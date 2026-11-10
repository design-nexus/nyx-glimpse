import QtQuick
import QtQuick.Effects
import Quickshell
import Nyx.Shell
import Nyx.Ui

// Glimpse's plugins (`nyx-glimpse --plugins`, the panel's plugin button), as the GTK
// window has them: Installed (each plugin's card: its settings, Update, Roll back,
// Remove with a second click, the switch that approves its permissions, what it may
// touch and its last error) and Browse (the catalog, and installing from Git). + adds
// one from a folder; the refresh button reloads the catalog. Every action runs in the
// backend; the line at the bottom says how it went.
Scope {
    id: pm

    required property var backend
    readonly property var manager: backend.state.manager || ({ open: false, serial: 0 })
    readonly property var plugins: backend.state.plugins || ({ installed: [], catalog: [], note: "" })
    readonly property bool asked: manager.open === true && backend.connected
    property string tab: "installed"
    // The plugin whose settings page shows, and that page's rows.
    property string settingsFor: ""
    property string settingsName: ""
    property var settingsRows: []
    property string settingsNote: ""

    function close() {
        backend.request("ClosePlugins", {}, null);
    }
    function op(name, body) {
        backend.request("PluginOp", Object.assign({ op: name }, body || {}), null);
    }
    function openSettings(id, name) {
        settingsFor = id;
        settingsName = name;
        settingsRows = [];
        settingsNote = "Loading settings…";
        backend.request("PluginSettings", { id: id }, (ok, err) => {
            if (pm.settingsFor !== id)
                return;
            if (err) {
                pm.settingsNote = err.msg;
            } else {
                pm.settingsNote = "Plugin settings";
                pm.settingsRows = (ok.rows || []).filter(r => r.key && r.key.length <= 128).slice(0, 32);
            }
        });
    }

    onAskedChanged: {
        if (asked) {
            settingsFor = "";
            ShellCore.showOverlay("glimpse-plugins");
        } else {
            ShellCore.hideOverlay("glimpse-plugins");
        }
    }

    OverlaySurface {
        id: surface
        overlayId: "glimpse-plugins"
        category: "transient"
        keyboard: "exclusive"
        fullscreenRule: "show"
        placement: "active"
        restoreFocus: true
        anchors.top: true
        anchors.bottom: true
        anchors.left: true
        anchors.right: true

        onShownChanged: {
            if (shown)
                card.forceActiveFocus();
            else if (pm.asked)
                pm.close();
        }

        MouseArea {
            anchors.fill: parent
            onPressed: pm.close()
        }

        MultiEffect {
            anchors.fill: card
            source: card
            shadowEnabled: true
            shadowColor: Qt.rgba(0, 0, 0, 0.38)
            shadowBlur: 1.0
            shadowVerticalOffset: 12
            blurMax: 40
            opacity: card.opacity
        }
        Rectangle {
            id: card
            width: 720
            height: 560
            anchors.centerIn: parent
            radius: Style.radiusDialog
            color: Style.bg
            border.width: 1
            border.color: Style.alpha(Style.text, 0.06)
            clip: true
            opacity: surface.shown ? 1 : 0
            Behavior on opacity {
                NumberAnimation {
                    duration: Style.reducedMotion ? 0 : Style.fast
                }
            }
            MouseArea {
                anchors.fill: parent
                onPressed: card.forceActiveFocus()
            }

            // Header: the app, the tabs, then add, refresh and close.
            Item {
                id: header
                width: parent.width
                height: 56
                Row {
                    anchors.left: parent.left
                    anchors.leftMargin: 16
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 10
                    Icon {
                        anchors.verticalCenter: parent.verticalCenter
                        name: pm.settingsFor !== "" ? "go-previous-symbolic" : "application-x-addon-symbolic"
                        size: 18
                        color: pm.settingsFor !== "" ? Style.dimText : Style.accent
                        MouseArea {
                            anchors.fill: parent
                            anchors.margins: -6
                            enabled: pm.settingsFor !== ""
                            cursorShape: Qt.PointingHandCursor
                            onClicked: pm.settingsFor = ""
                        }
                    }
                    Column {
                        anchors.verticalCenter: parent.verticalCenter
                        Label {
                            text: "Glimpse"
                            role: "title"
                        }
                        Label {
                            text: pm.settingsFor !== "" ? pm.settingsName + " settings" : "Plugins"
                            role: "caption"
                            dim: true
                        }
                    }
                }
                Segmented {
                    visible: pm.settingsFor === ""
                    anchors.centerIn: parent
                    width: 220
                    options: [{ id: "installed", label: "Installed" }, { id: "browse", label: "Browse" }]
                    current: pm.tab
                    onChosen: id => pm.tab = id
                }
                Row {
                    anchors.right: parent.right
                    anchors.rightMargin: 12
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 4
                    IconButton {
                        visible: pm.settingsFor === ""
                        icon: "list-add-symbolic"
                        tooltip: "Add a plugin from a folder"
                        onClicked: pm.op("add_local")
                    }
                    IconButton {
                        visible: pm.settingsFor === ""
                        icon: "view-refresh-symbolic"
                        tooltip: "Refresh the catalog"
                        onClicked: pm.op("refresh")
                    }
                    IconButton {
                        icon: "window-close-symbolic"
                        tooltip: "Close (Esc)"
                        onClicked: pm.close()
                    }
                }
            }

            Flickable {
                id: page
                y: header.height
                width: parent.width
                height: parent.height - header.height - note.height
                clip: true
                contentHeight: pages.height
                boundsBehavior: Flickable.StopAtBounds

                Column {
                    id: pages
                    width: page.width
                    padding: 16
                    topPadding: 8
                    spacing: 12

                    // ---------- Installed ----------
                    Column {
                        visible: pm.settingsFor === "" && pm.tab === "installed"
                        width: parent.width - 32
                        spacing: 12
                        StateView {
                            visible: pm.plugins.installed.length === 0
                            width: parent.width
                            height: 300
                            icon: "application-x-addon-symbolic"
                            text: "No plugins yet. Install one from Browse, or add a plugin from a folder with +."
                        }
                        Repeater {
                            model: pm.plugins.installed
                            delegate: PluginCard {
                                width: parent.width
                                onSettings: pm.openSettings(modelData.id, modelData.name)
                                onOp: (name, body) => pm.op(name, Object.assign({ id: modelData.id }, body || {}))
                            }
                        }
                    }

                    // ---------- Browse ----------
                    Column {
                        visible: pm.settingsFor === "" && pm.tab === "browse"
                        width: parent.width - 32
                        spacing: 8
                        Label {
                            text: "Catalog"
                            role: "group"
                            color: Style.tertiary
                        }
                        Rectangle {
                            width: parent.width
                            height: catalog.height + 8
                            radius: Style.radiusCard
                            color: Style.card
                            Column {
                                id: catalog
                                x: 14
                                y: 4
                                width: parent.width - 28
                                Label {
                                    visible: pm.plugins.catalog.length === 0
                                    width: parent.width
                                    topPadding: 10
                                    bottomPadding: 10
                                    text: "The catalog isn't loaded. Refresh it with the button above."
                                    wrapMode: Text.Wrap
                                    role: "secondary"
                                    dim: true
                                }
                                Repeater {
                                    model: pm.plugins.catalog
                                    delegate: Item {
                                        required property var modelData
                                        width: catalog.width
                                        height: Math.max(52, about.height + 16)
                                        Column {
                                            id: about
                                            anchors.left: parent.left
                                            anchors.right: action.left
                                            anchors.rightMargin: 12
                                            anchors.verticalCenter: parent.verticalCenter
                                            spacing: 2
                                            Label {
                                                width: parent.width
                                                text: modelData.name
                                                strong: true
                                            }
                                            Label {
                                                width: parent.width
                                                text: modelData.description
                                                wrapMode: Text.Wrap
                                                size: 12.5
                                                dim: true
                                            }
                                        }
                                        Item {
                                            id: action
                                            anchors.right: parent.right
                                            anchors.verticalCenter: parent.verticalCenter
                                            width: modelData.installed ? tag.width : install.width
                                            height: 30
                                            Badge {
                                                id: tag
                                                visible: modelData.installed
                                                anchors.verticalCenter: parent.verticalCenter
                                                text: "Installed"
                                                tone: "neutral"
                                            }
                                            InstallButton {
                                                id: install
                                                visible: !modelData.installed
                                                text: "Install"
                                                onClicked: pm.op("install", { id: modelData.id })
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        Label {
                            topPadding: 10
                            text: "From Git"
                            role: "group"
                            color: Style.tertiary
                        }
                        Rectangle {
                            width: parent.width
                            height: git.height + 28
                            radius: Style.radiusCard
                            color: Style.card
                            Column {
                                id: git
                                x: 14
                                y: 14
                                width: parent.width - 28
                                spacing: 8
                                Field {
                                    id: url
                                    width: parent.width
                                    placeholder: "HTTPS Git repository"
                                }
                                Row {
                                    width: parent.width
                                    spacing: 8
                                    Field {
                                        id: revision
                                        width: (parent.width - 8) / 2
                                        placeholder: "Full 40-character commit hash"
                                    }
                                    Field {
                                        id: folder
                                        width: (parent.width - 8) / 2
                                        placeholder: "Plugin subfolder (optional)"
                                    }
                                }
                                Item {
                                    width: parent.width
                                    height: 30
                                    InstallButton {
                                        anchors.right: parent.right
                                        text: "Install from Git"
                                        onClicked: pm.op("install_git", { url: url.text, revision: revision.text, folder: folder.text })
                                    }
                                }
                            }
                        }
                    }

                    // ---------- A plugin's settings ----------
                    Column {
                        visible: pm.settingsFor !== ""
                        width: parent.width - 32
                        spacing: 12
                        Label {
                            width: parent.width
                            text: pm.settingsNote
                            wrapMode: Text.Wrap
                            dim: true
                        }
                        Repeater {
                            model: pm.settingsFor !== "" ? pm.settingsRows : []
                            delegate: PluginSetting {
                                width: parent.width
                                onSave: value => pm.backend.request("PluginConfigure", { id: pm.settingsFor, key: modelData.key, value: value }, (ok, err) => {
                                    pm.settingsNote = err ? err.msg : "Saved";
                                })
                            }
                        }
                    }
                }
            }

            Text {
                id: note
                anchors.bottom: parent.bottom
                width: parent.width
                leftPadding: 18
                rightPadding: 18
                topPadding: 8
                bottomPadding: 12
                text: pm.plugins.note || ""
                wrapMode: Text.Wrap
                color: Style.tertiary
                font.family: Style.font
                font.pointSize: 11.5 * 0.75
            }
        }
    }

    component InstallButton: Rectangle {
        id: ib
        property string text: ""
        signal clicked
        width: ibLabel.implicitWidth + 28
        height: 30
        radius: Style.radiusSmall
        color: Style.alpha(Style.accent, ibArea.containsMouse ? 0.24 : 0.16)
        Text {
            id: ibLabel
            anchors.centerIn: parent
            text: ib.text
            color: Style.accent
            font.family: Style.font
            font.pixelSize: 13
            font.weight: Font.Medium
        }
        MouseArea {
            id: ibArea
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: ib.clicked()
        }
    }
}

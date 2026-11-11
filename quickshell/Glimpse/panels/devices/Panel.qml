import QtQuick
import Nyx.Ui

// Phones and other devices paired through Flux (Nyx Connect), connected first, with
// their battery; the words after `devices` keep those whose name starts with them. Up
// and Down pick one, R rings it, S sends it the clipboard (asking first: it leaves this
// computer), Enter opens Settings › Devices, where pairing and access are.
FocusScope {
    id: panel

    property var host: null
    // Across the top of Glimpse: how many are connected on the left, the list beside it.
    readonly property bool wide: host !== null && host.compact
    readonly property var live: host ? host.topic("devices") : ({})
    readonly property var all: (live.state && live.state.items) || []
    readonly property string failure: (live.state && live.state.error) || ""
    readonly property string filter: host ? host.query.trim().toLowerCase() : ""
    readonly property var shown: all.filter(d => filter === "" || d.title.toLowerCase().startsWith(filter))
    property int picked: 0
    readonly property var pickedOne: shown[Math.min(picked, shown.length - 1)] || null

    function act(action, d) {
        if (!d)
            return;
        host.run("devices." + action, { device: d.id }, (result, err) => {
            if (result)
                host.say(result.message);
        });
    }

    Keys.onPressed: e => {
        if (e.key === Qt.Key_Down)
            picked = Math.min(shown.length - 1, picked + 1);
        else if (e.key === Qt.Key_Up)
            picked = Math.max(0, picked - 1);
        else if (e.key === Qt.Key_R && pickedOne && pickedOne.can_ring)
            act("ring", pickedOne);
        else if (e.key === Qt.Key_S && pickedOne && pickedOne.can_send)
            act("send", pickedOne);
        else if (e.key === Qt.Key_Return || e.key === Qt.Key_Enter)
            host.open({});
        else
            return;
        e.accepted = true;
    }

    StateView {
        anchors.centerIn: parent
        width: parent.width
        visible: panel.failure !== ""
        kind: "error"
        text: panel.failure
        actionText: "Open Devices"
        onAction: panel.host.open({})
    }

    Column {
        visible: panel.wide && panel.failure === ""
        width: footer.width
        spacing: 2
        Label {
            text: panel.all.filter(d => d.connected).length + " connected"
            role: "title"
        }
        Label {
            text: "of " + panel.all.length + " paired"
            dim: true
        }
    }

    ListView {
        id: list
        visible: panel.failure === ""
        x: panel.wide ? footer.width + 16 : 0
        anchors.top: parent.top
        anchors.bottom: panel.wide ? parent.bottom : footer.top
        anchors.bottomMargin: panel.wide ? 0 : 6
        width: parent.width - x
        clip: true
        model: panel.shown
        delegate: Rectangle {
            id: row
            required property var modelData
            required property int index
            width: list.width
            height: 48
            radius: Style.radiusSmall
            color: index === panel.picked ? Style.selected : "transparent"
            Accessible.role: Accessible.ListItem
            Accessible.name: modelData.title + ", " + modelData.subtitle
            Accessible.selected: index === panel.picked
            Icon {
                id: kind
                x: 10
                anchors.verticalCenter: parent.verticalCenter
                name: row.modelData.icon || "phone-symbolic"
                size: 20
                color: row.modelData.connected ? Style.accent : Style.dimText
            }
            Column {
                anchors.left: kind.right
                anchors.leftMargin: 10
                anchors.right: buttons.left
                anchors.rightMargin: 8
                anchors.verticalCenter: parent.verticalCenter
                Label {
                    width: parent.width
                    text: row.modelData.title
                    elide: Text.ElideRight
                }
                Label {
                    width: parent.width
                    text: row.modelData.subtitle
                    role: "caption"
                    dim: true
                    elide: Text.ElideRight
                }
            }
            Row {
                id: buttons
                anchors.right: parent.right
                anchors.rightMargin: 6
                anchors.verticalCenter: parent.verticalCenter
                spacing: 2
                IconButton {
                    visible: row.modelData.can_ring
                    icon: "audio-volume-high-symbolic"
                    tooltip: "Ring " + row.modelData.title + " (R)"
                    onClicked: panel.act("ring", row.modelData)
                }
                IconButton {
                    visible: row.modelData.can_send
                    icon: "edit-paste-symbolic"
                    tooltip: "Send the clipboard to " + row.modelData.title + " (S)"
                    onClicked: panel.act("send", row.modelData)
                }
            }
            MouseArea {
                anchors.fill: parent
                z: -1
                onClicked: panel.picked = row.index
            }
        }
        StateView {
            anchors.centerIn: parent
            width: parent.width
            visible: panel.shown.length === 0 && panel.live.state !== undefined && panel.failure === ""
            kind: "empty"
            icon: "phone-symbolic"
            text: panel.filter === "" ? "No paired devices yet" : "No device called “" + panel.host.query + "…”"
            actionText: "Pair one in Settings"
            onAction: panel.host.open({})
        }
    }

    Item {
        id: footer
        anchors.bottom: parent.bottom
        width: panel.wide ? 200 : parent.width
        height: openSettings.height
        Label {
            visible: !panel.wide
            anchors.verticalCenter: parent.verticalCenter
            text: panel.all.filter(d => d.connected).length + " of " + panel.all.length + " connected"
            role: "caption"
            dim: true
        }
        Button {
            id: openSettings
            anchors.right: panel.wide ? undefined : parent.right
            text: "Open Devices"
            onClicked: panel.host.open({})
        }
    }
}

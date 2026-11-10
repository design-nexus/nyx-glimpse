import QtQuick
import Nyx.Ui

// An installed plugin: name, version and description; its settings, Update, Roll back
// and Remove (a second click within four seconds removes it); the switch (turning it on
// approves the permissions listed); what it may touch; its last error.
Rectangle {
    id: c

    required property var modelData

    signal settings
    signal op(string name, var body)

    height: body.height + 28
    radius: Style.radiusCard
    color: Style.card

    property bool armed: false
    Timer {
        id: disarm
        interval: 4000
        onTriggered: c.armed = false
    }

    Column {
        id: body
        x: 14
        y: 14
        width: parent.width - 28
        spacing: 8

        Item {
            width: parent.width
            height: Math.max(text.height, 32)
            Icon {
                id: icon
                y: 1
                name: "application-x-addon-symbolic"
                size: 18
                color: Style.accent
            }
            Column {
                id: text
                anchors.left: icon.right
                anchors.leftMargin: 12
                anchors.right: actions.left
                anchors.rightMargin: 6
                spacing: 2
                Row {
                    spacing: 8
                    Label {
                        text: c.modelData.name
                        strong: true
                    }
                    Label {
                        anchors.baseline: parent.children[0].baseline
                        text: c.modelData.version
                        size: 11.5
                        color: Style.tertiary
                    }
                }
                Label {
                    visible: c.modelData.description !== ""
                    width: parent.width
                    text: c.modelData.description
                    wrapMode: Text.Wrap
                    size: 12.5
                    dim: true
                }
            }
            Row {
                id: actions
                anchors.right: parent.right
                spacing: 2
                IconButton {
                    visible: c.modelData.settings
                    icon: "nyx-settings-symbolic"
                    tooltip: "Plugin settings"
                    onClicked: c.settings()
                }
                IconButton {
                    icon: "folder-download-symbolic"
                    tooltip: "Update"
                    onClicked: c.op("update", {})
                }
                IconButton {
                    icon: "edit-undo-symbolic"
                    tooltip: "Roll back to the previous version"
                    onClicked: c.op("rollback", {})
                }
                IconButton {
                    icon: "user-trash-symbolic"
                    danger: c.armed
                    toggled: c.armed
                    tooltip: c.armed ? "Click again to remove" : "Remove"
                    onClicked: {
                        if (c.armed) {
                            c.armed = false;
                            c.op("remove", {});
                        } else {
                            c.armed = true;
                            disarm.restart();
                        }
                    }
                }
                Toggle {
                    anchors.verticalCenter: parent.verticalCenter
                    checked: c.modelData.enabled
                    onToggled: on => c.op("enable", { on: on })
                    Tip {
                        text: c.modelData.enabled ? "Turn off" : "Turn on: approves the permissions listed below"
                        shown: parent.hovered === true
                    }
                }
            }
        }

        // What it may touch, as chips under the description.
        Flow {
            x: 30
            width: parent.width - 30
            spacing: 6
            Repeater {
                model: c.modelData.chips
                delegate: Rectangle {
                    required property string modelData
                    width: Math.min(chip.implicitWidth + 16, 260)
                    height: 22
                    radius: Style.radiusSmall
                    color: Style.field
                    Text {
                        id: chip
                        anchors.centerIn: parent
                        width: Math.min(implicitWidth, 244)
                        text: parent.modelData
                        elide: Text.ElideMiddle
                        color: Style.dimText
                        font.family: Style.font
                        font.pointSize: 11.5 * 0.75
                    }
                }
            }
        }

        Row {
            visible: c.modelData.error !== ""
            x: 30
            width: parent.width - 30
            spacing: 6
            Icon {
                name: "dialog-warning-symbolic"
                size: 14
                color: Style.warning
            }
            Label {
                width: parent.width - 20
                text: c.modelData.error
                wrapMode: Text.Wrap
                size: 12
                dim: true
            }
        }
    }
}

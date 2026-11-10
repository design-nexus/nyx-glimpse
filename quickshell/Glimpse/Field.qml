import QtQuick
import Nyx.Ui

// A one-line text field with a placeholder: `field` fill, an accent outline in focus.
Rectangle {
    id: f

    property alias text: input.text
    property string placeholder: ""
    property alias input: input

    signal accepted

    implicitWidth: 200
    implicitHeight: Style.controlHeight + 2
    radius: Style.radiusSmall
    color: Style.field
    border.width: input.activeFocus ? 2 : 1
    border.color: input.activeFocus ? Style.accent : Style.alpha(Style.border, 0.5)

    TextInput {
        id: input
        anchors.left: parent.left
        anchors.leftMargin: 10
        anchors.right: parent.right
        anchors.rightMargin: 10
        anchors.verticalCenter: parent.verticalCenter
        color: Style.text
        selectionColor: Style.alpha(Style.accent, 0.4)
        selectedTextColor: Style.text
        font.family: Style.font
        font.pixelSize: 13
        clip: true
        selectByMouse: true
        onAccepted: f.accepted()
        Text {
            anchors.fill: parent
            verticalAlignment: Text.AlignVCenter
            visible: input.text === "" && !input.preeditText
            text: f.placeholder
            color: Style.tertiary
            font: input.font
        }
    }
    MouseArea {
        anchors.fill: parent
        cursorShape: Qt.IBeamCursor
        onPressed: m => {
            input.forceActiveFocus();
            m.accepted = false;
        }
    }
}

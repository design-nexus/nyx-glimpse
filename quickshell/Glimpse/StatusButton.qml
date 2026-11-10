import QtQuick
import Nyx.Ui

// One of the hint line's buttons: a quiet 28 px icon that lights on hover.
Rectangle {
    id: b

    property string icon: ""
    property string tooltip: ""

    signal clicked

    width: 28
    height: 28
    radius: Style.radiusSmall
    color: area.containsMouse ? Style.rowHover : "transparent"

    Icon {
        anchors.centerIn: parent
        name: b.icon
        size: 16
        recolor: true
        color: area.containsMouse ? Style.text : Style.tertiary
    }
    MouseArea {
        id: area
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: b.clicked()
    }
    Tip {
        text: b.tooltip
        shown: area.containsMouse
    }
}

import QtQuick
import Nyx.Ui

// A key in the hint line: a small quiet cap.
Rectangle {
    property alias text: label.text

    implicitWidth: Math.max(18, label.implicitWidth + 10)
    implicitHeight: 18
    radius: 5
    color: Style.alpha(Style.text, 0.07)

    Text {
        id: label
        anchors.centerIn: parent
        color: Style.dimText
        font.family: Style.font
        font.pointSize: 10.5 * 0.75
        font.weight: Font.Medium
    }
}

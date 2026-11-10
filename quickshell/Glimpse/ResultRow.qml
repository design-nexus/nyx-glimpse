import QtQuick
import Nyx.Ui

// One result, under its category's heading when it's the first of it: the icon (an
// app's own, otherwise the kind's symbolic), the title, a second line (a file's folder
// keeps its end, which tells two README.md files apart), a star for a favorite and the
// tag saying where it matched.
Item {
    id: r

    required property var modelData
    required property int index
    required property var rows
    property bool selected: false
    readonly property bool heading: index === 0 || rows[index - 1].order !== modelData.order
    readonly property bool double: modelData.subtitle !== ""

    signal clicked
    signal pressedRow

    implicitHeight: (heading ? head.height : 0) + body.height + 2

    Text {
        id: head
        visible: r.heading
        x: 18
        width: parent.width - 36
        topPadding: r.index === 0 ? 4 : 20
        bottomPadding: 4
        text: r.modelData.category.toUpperCase()
        color: Style.tertiary
        font.family: Style.font
        font.pixelSize: 11
        font.weight: Font.DemiBold
        font.letterSpacing: 11 * 0.06
    }

    Rectangle {
        id: body
        x: 8
        y: (r.heading ? head.height : 0) + 1
        width: parent.width - 16
        height: r.double ? 44 : 36
        radius: Style.radiusSmall
        color: r.selected ? Style.selected : area.containsMouse ? Style.rowHover : "transparent"
        Behavior on color {
            ColorAnimation {
                duration: 120
            }
        }

        Icon {
            id: icon
            x: 10
            anchors.verticalCenter: parent.verticalCenter
            name: r.modelData.icon
            size: 20
            recolor: r.modelData.kind !== "app" || r.modelData.icon.endsWith("-symbolic")
            color: r.selected ? Style.accent : Style.dimText
        }

        Column {
            anchors.left: icon.right
            anchors.leftMargin: 10
            anchors.right: trail.left
            anchors.rightMargin: 8
            anchors.verticalCenter: parent.verticalCenter
            spacing: 1
            Text {
                width: parent.width
                text: r.modelData.title
                color: Style.text
                font.family: Style.font
                font.pixelSize: 13
                elide: Text.ElideRight
            }
            Text {
                width: parent.width
                visible: r.double
                text: r.modelData.subtitle
                color: r.selected ? Style.mix(Style.dimText, Style.text, 0.35) : Style.dimText
                font.family: Style.font
                font.pixelSize: 12
                elide: r.modelData.kind === "file" ? Text.ElideLeft : Text.ElideRight
            }
        }

        Row {
            id: trail
            anchors.right: parent.right
            anchors.rightMargin: 10
            anchors.verticalCenter: parent.verticalCenter
            spacing: 6
            Icon {
                anchors.verticalCenter: parent.verticalCenter
                visible: r.modelData.favorite === true
                name: "starred-symbolic"
                size: 14
                color: r.selected ? Style.accent : Style.dimText
            }
            Badge {
                anchors.verticalCenter: parent.verticalCenter
                visible: r.modelData.reason !== ""
                text: r.modelData.reason
                tone: r.modelData.related ? "neutral" : "accent"
            }
        }

        MouseArea {
            id: area
            anchors.fill: parent
            hoverEnabled: true
            onPressed: r.pressedRow()
            onClicked: r.clicked()
        }
        Tip {
            text: r.modelData.tooltip || ""
            shown: area.containsMouse && text !== ""
        }
    }
}

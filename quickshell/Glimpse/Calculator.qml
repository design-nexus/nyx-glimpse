import QtQuick
import Nyx.Ui

// The calculator a calculation shows: the answer, a keypad that edits the search, and
// Copy answer.
Column {
    id: calc

    property string display: ""

    signal key(string k)
    signal copyAnswer

    spacing: 12

    TextEdit {
        width: parent.width
        topPadding: 10
        bottomPadding: 14
        leftPadding: 4
        rightPadding: 4
        text: calc.display
        readOnly: true
        selectByMouse: true
        wrapMode: TextEdit.Wrap
        horizontalAlignment: TextEdit.AlignRight
        color: Style.text
        selectionColor: Style.alpha(Style.accent, 0.4)
        font.family: Style.font
        font.pixelSize: 22
        font.weight: Font.DemiBold
        font.features: { "tnum": 1 }
    }

    Grid {
        id: keys
        width: parent.width
        columns: 4
        rowSpacing: 6
        columnSpacing: 6
        Repeater {
            model: ["C", "⌫", "(", ")", "7", "8", "9", "÷", "4", "5", "6", "×", "1", "2", "3", "−", "0", ".", "=", "+"]
            delegate: Rectangle {
                required property string modelData
                width: (keys.width - 18) / 4
                height: 32
                radius: Style.radiusSmall
                color: area.pressed ? Style.mix(Style.card, Style.text, 0.12) : area.containsMouse ? Style.mix(Style.card, Style.text, 0.06) : Style.card
                Text {
                    anchors.centerIn: parent
                    text: parent.modelData
                    color: Style.text
                    font.family: Style.font
                    font.pixelSize: 14
                    font.weight: Font.DemiBold
                    font.features: { "tnum": 1 }
                }
                MouseArea {
                    id: area
                    anchors.fill: parent
                    hoverEnabled: true
                    onClicked: calc.key(parent.modelData)
                }
            }
        }
    }

    Rectangle {
        width: parent.width
        height: 32
        radius: Style.radiusSmall
        color: copyArea.pressed ? Style.mix(Style.card, Style.text, 0.12) : copyArea.containsMouse ? Style.mix(Style.card, Style.text, 0.06) : Style.card
        Text {
            anchors.centerIn: parent
            text: "Copy answer"
            color: Style.text
            font.family: Style.font
            font.pixelSize: 13
        }
        MouseArea {
            id: copyArea
            anchors.fill: parent
            hoverEnabled: true
            onClicked: calc.copyAnswer()
        }
    }
}

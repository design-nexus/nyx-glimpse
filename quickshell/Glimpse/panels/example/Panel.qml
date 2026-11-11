import QtQuick
import Nyx.Ui

// A test panel (offered only to a backend started with GLIMPSE_TEST_PANELS=1): the
// app's first topic as fields, and its `example.ping` action.
FocusScope {
    id: panel

    property var host: null
    readonly property var topics: host && host.offer ? host.offer.topics : []
    readonly property var live: host && topics.length > 0 ? host.topic(topics[0]) : ({})
    readonly property var fields: live.state && typeof live.state === "object" ? Object.keys(live.state).map(k => [k, JSON.stringify(live.state[k])]) : []
    property string answer: ""

    Keys.onReturnPressed: ping.clicked()

    Column {
        anchors.fill: parent
        anchors.margins: panel.host && panel.host.compact ? 8 : 18
        spacing: 12

        Label {
            text: panel.host ? panel.host.offer.name + (panel.host.query ? " · " + panel.host.query : "") : ""
            role: "title"
        }
        Repeater {
            model: panel.fields
            delegate: Label {
                required property var modelData
                text: modelData[0] + ": " + modelData[1]
            }
        }
        Button {
            id: ping
            text: "Ping"
            focus: true
            onClicked: panel.host.run("example.ping", { text: panel.host.query }, (result, err) => panel.answer = err || JSON.stringify(result))
        }
        Label {
            visible: panel.answer !== ""
            text: panel.answer
            dim: true
        }
    }
}

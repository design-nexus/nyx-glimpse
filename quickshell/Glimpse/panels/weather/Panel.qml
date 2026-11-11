import QtQuick
import Nyx.Ui

// The weather now and the next five days, for the active location, in the units
// Weather is set to; the words after `weather` ask about another place or day
// (`weather tomorrow`, `weather London`). Enter opens Weather, Ctrl+R refreshes.
FocusScope {
    id: panel

    property var host: null
    // Across the top of Glimpse: the weather now on the left, the days beside it.
    readonly property bool wide: host !== null && host.compact
    readonly property var live: host ? host.topic("now") : ({})
    readonly property var now: live.state || ({})
    readonly property string failure: now.error || ""
    readonly property bool shown: failure === "" && now.temperature !== undefined
    readonly property string words: host ? host.query.trim() : ""
    property var answer: null

    function ask() {
        if (words === "") {
            answer = null;
            return;
        }
        const asked = words;
        host.run("weather.query", { query: asked }, (result, err) => {
            if (asked === panel.words)
                panel.answer = result ? result : { error: err };
        });
    }
    function day(date) {
        const p = (date || "").split("-").map(Number);
        const d = new Date(p[0], p[1] - 1, p[2]);
        return d.toDateString() === new Date().toDateString() ? "Today" : d.toLocaleDateString(Qt.locale(), "ddd");
    }

    Timer {
        id: typing
        interval: 400
        onTriggered: panel.ask()
    }
    onWordsChanged: typing.restart()
    Component.onCompleted: ask()

    Keys.onPressed: e => {
        const ctrl = (e.modifiers & Qt.ControlModifier) !== 0;
        if (e.key === Qt.Key_Return || e.key === Qt.Key_Enter)
            host.open({});
        else if (ctrl && e.key === Qt.Key_R)
            host.run("weather.refresh", {}, null);
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
        actionText: "Open Weather"
        onAction: panel.host.open({})
    }

    Column {
        id: main
        visible: panel.shown
        width: panel.wide ? 300 : parent.width
        spacing: panel.wide ? 10 : 14

        Row {
            width: parent.width
            spacing: 14
            Icon {
                anchors.verticalCenter: parent.verticalCenter
                name: panel.now.icon || "weather-few-clouds-symbolic"
                size: 48
                color: Style.accent
            }
            Column {
                anchors.verticalCenter: parent.verticalCenter
                width: parent.width - 62
                Label {
                    text: panel.now.temperature || ""
                    role: "readout"
                }
                Label {
                    width: parent.width
                    text: (panel.now.condition || "") + " · " + (panel.now.location || "")
                    dim: true
                    elide: Text.ElideRight
                }
            }
        }

        Label {
            width: parent.width
            text: "Feels like " + (panel.now.feels_like || "") + " · High " + (panel.now.high || "") + " · Low " + (panel.now.low || "") + " · Wind " + (panel.now.wind || "") + (panel.now.humidity_pct !== undefined && panel.now.humidity_pct !== null ? " · Humidity " + panel.now.humidity_pct + "%" : "")
            role: "caption"
            dim: true
            wrapMode: Text.Wrap
        }

        Repeater {
            model: (panel.now.alerts || []).slice(0, panel.wide ? 1 : 2)
            delegate: Label {
                required property var modelData
                width: parent.width
                text: "⚠ " + (modelData.headline || modelData.event || "Weather alert")
                color: Style.warning
                wrapMode: Text.Wrap
                maximumLineCount: 2
                elide: Text.ElideRight
            }
        }
    }

    Row {
        id: days
        visible: panel.shown
        x: panel.wide ? main.width + 16 : 0
        y: panel.wide ? 0 : main.height + 14
        width: panel.wide ? parent.width - main.width - 16 : parent.width
        spacing: 6
        Repeater {
            model: panel.now.days || []
            delegate: Card {
                required property var modelData
                Accessible.role: Accessible.StaticText
                Accessible.name: panel.day(modelData.date) + ", " + modelData.condition + ", " + modelData.high + " / " + modelData.low + (modelData.rain_pct > 0 ? ", " + modelData.rain_pct + "% rain" : "")
                width: (days.width - 6 * 4) / 5
                height: panel.wide ? Math.max(104, panel.height - footer.height - 12 - (panel.answer !== null ? 48 : 0)) : 104
                Column {
                    anchors.centerIn: parent
                    spacing: 4
                    Label {
                        anchors.horizontalCenter: parent.horizontalCenter
                        text: panel.day(modelData.date)
                        role: "caption"
                        dim: true
                    }
                    Icon {
                        anchors.horizontalCenter: parent.horizontalCenter
                        name: modelData.icon
                        size: 22
                    }
                    Label {
                        anchors.horizontalCenter: parent.horizontalCenter
                        text: modelData.high + " / " + modelData.low
                        role: "caption"
                    }
                    Label {
                        anchors.horizontalCenter: parent.horizontalCenter
                        visible: modelData.rain_pct > 0
                        text: modelData.rain_pct + "% rain"
                        role: "caption"
                        dim: true
                    }
                }
            }
        }
    }

    // The answer to the words typed after `weather`.
    Column {
        visible: panel.shown && panel.answer !== null
        x: days.x
        y: days.y + days.height + (panel.wide ? 10 : 14)
        width: days.width
        spacing: 2
        Label {
            width: parent.width
            text: panel.answer ? (panel.answer.title || panel.answer.error || "") : ""
            strong: true
            wrapMode: Text.Wrap
            maximumLineCount: panel.wide ? 1 : 3
            elide: Text.ElideRight
        }
        Label {
            width: parent.width
            text: panel.answer ? panel.answer.subtitle || "" : ""
            dim: true
            wrapMode: Text.Wrap
            maximumLineCount: panel.wide ? 1 : 3
            elide: Text.ElideRight
        }
    }

    Item {
        id: footer
        anchors.bottom: parent.bottom
        width: parent.width
        height: openWeather.height
        Label {
            anchors.verticalCenter: parent.verticalCenter
            width: parent.width - openWeather.width - 12
            text: panel.now.attribution || ""
            role: "caption"
            dim: true
        }
        Button {
            id: openWeather
            anchors.right: parent.right
            text: "Open Weather"
            onClicked: panel.host.open({})
        }
    }
}

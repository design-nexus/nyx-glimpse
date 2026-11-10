import QtQuick
import Nyx.Ui

// Home at a glance: scenes, favorite devices first, then each room's (filtered by the
// words after `home`: a room, a device, or `lights`), thermostats and routines. Up and
// Down pick a device, Space or Enter switches it, Left and Right dim a light.
FocusScope {
    id: panel

    property var host: null
    readonly property var live: host ? host.topic("home") : ({})
    readonly property var home: live.state || ({})
    readonly property string failure: home.error || ""
    readonly property string filter: host ? host.query.toLowerCase() : ""

    function matches(d) {
        if (filter === "")
            return true;
        if ((filter === "lights" || filter === "light") && d.domain === "light")
            return true;
        return d.name.toLowerCase().indexOf(filter) >= 0 || (d.room || "").toLowerCase().indexOf(filter) >= 0;
    }
    // Favorites, then rooms in Home Assistant's order, then devices without a room.
    readonly property var rows: {
        const out = [];
        const devices = (home.devices || []).filter(d => matches(d));
        const fav = devices.filter(d => d.favorite);
        if (fav.length > 0) {
            out.push({ heading: "Favorites" });
            fav.forEach(d => out.push({ device: d }));
        }
        const rooms = (home.rooms || []).concat([""]);
        rooms.forEach(room => {
            const inRoom = devices.filter(d => !d.favorite && (d.room || "") === room);
            if (inRoom.length === 0)
                return;
            out.push({ heading: room || "Elsewhere" });
            inRoom.forEach(d => out.push({ device: d }));
        });
        return out;
    }
    readonly property var deviceRows: rows.map((r, i) => r.device ? i : -1).filter(i => i >= 0)
    readonly property var scenes: (home.scenes || []).filter(s => filter === "" || s.name.toLowerCase().indexOf(filter) >= 0 || (s.room || "").toLowerCase().indexOf(filter) >= 0)
    readonly property var routines: home.routines || []
    property int picked: 0
    readonly property var pickedDevice: deviceRows.length > 0 ? rows[deviceRows[Math.min(picked, deviceRows.length - 1)]].device : null

    function switchDevice(d, on) {
        host.run("home.switch", { device: d.id, on: on }, (result, err) => {
            if (result && result.message)
                host.say(result.message);
        });
    }
    function brightness(d, pct) {
        host.run("home.light", { device: d.id, brightness: Math.max(1, Math.min(100, Math.round(pct))) }, null);
    }
    function climate(d, delta) {
        const t = (d.target || d.current || 20) + delta;
        host.run("home.climate", { device: d.id, temperature: t }, (result, err) => {
            if (result && result.message)
                host.say(result.message);
        });
    }
    function icon(d) {
        return ({ light: "display-brightness-symbolic", "switch": "system-shutdown-symbolic", fan: "weather-windy-symbolic", climate: "weather-few-clouds-symbolic" })[d.domain] || "emblem-system-symbolic";
    }
    function stateText(d) {
        if (!d.available)
            return "Unavailable";
        if (d.domain === "climate")
            return (d.current !== undefined && d.current !== null ? d.current + "°" : "") + (d.target !== undefined && d.target !== null ? " → " + d.target + "°" : "");
        let s = d.on ? "On" : "Off";
        if (d.on && d.brightness !== undefined && d.brightness !== null)
            s += " · " + Math.round(d.brightness * 100) + "%";
        return s;
    }

    Keys.onPressed: e => {
        const d = pickedDevice;
        if (e.key === Qt.Key_Down)
            picked = Math.min(deviceRows.length - 1, picked + 1);
        else if (e.key === Qt.Key_Up)
            picked = Math.max(0, picked - 1);
        else if (d && (e.key === Qt.Key_Space || e.key === Qt.Key_Return || e.key === Qt.Key_Enter) && d.domain !== "climate")
            switchDevice(d, !d.on);
        else if (d && d.dimmable && (e.key === Qt.Key_Left || e.key === Qt.Key_Right))
            brightness(d, (d.on && d.brightness ? d.brightness * 100 : 0) + (e.key === Qt.Key_Right ? 10 : -10));
        else if (d && d.domain === "climate" && (e.key === Qt.Key_Left || e.key === Qt.Key_Right))
            climate(d, e.key === Qt.Key_Right ? 0.5 : -0.5);
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
        actionText: "Open Home"
        onAction: panel.host.open({})
    }

    Column {
        id: top
        visible: panel.failure === ""
        width: parent.width
        spacing: 8
        Label {
            visible: panel.scenes.length > 0
            text: "Scenes"
            role: "caption"
            dim: true
        }
        Flow {
            width: parent.width
            visible: panel.scenes.length > 0
            spacing: 6
            Repeater {
                model: panel.scenes.slice(0, 8)
                delegate: Button {
                    required property var modelData
                    text: modelData.name
                    onClicked: panel.host.run("home.scene", { scene: modelData.id }, (result, err) => {
                        if (result && result.message)
                            panel.host.say(result.message);
                    })
                }
            }
        }
    }

    ListView {
        id: list
        visible: panel.failure === ""
        anchors.top: top.bottom
        anchors.topMargin: 8
        anchors.bottom: footer.top
        anchors.bottomMargin: 6
        width: parent.width
        clip: true
        model: panel.rows
        delegate: Item {
            id: row
            required property var modelData
            required property int index
            readonly property var d: modelData.device || null
            readonly property bool chosen: d !== null && panel.deviceRows[panel.picked] === index
            Accessible.role: d ? Accessible.ListItem : Accessible.Heading
            Accessible.name: d ? d.name + ", " + panel.stateText(d) : (modelData.heading || "")
            Accessible.selected: chosen
            width: list.width
            height: d ? (d.dimmable && d.on ? 66 : 40) : 26
            Label {
                visible: !row.d
                anchors.bottom: parent.bottom
                anchors.bottomMargin: 4
                text: row.modelData.heading || ""
                role: "caption"
                dim: true
            }
            Rectangle {
                visible: row.d !== null
                anchors.fill: parent
                radius: Style.radiusSmall
                color: row.chosen ? Style.selected : "transparent"
                Icon {
                    id: kind
                    x: 10
                    y: 11
                    name: row.d ? panel.icon(row.d) : ""
                    size: 18
                    color: row.d && row.d.on ? Style.accent : Style.dimText
                }
                Column {
                    x: 38
                    y: 4
                    width: parent.width - 38 - trailing.width - 16
                    Label {
                        width: parent.width
                        text: row.d ? row.d.name : ""
                        elide: Text.ElideRight
                    }
                    Label {
                        width: parent.width
                        text: row.d ? panel.stateText(row.d) : ""
                        role: "caption"
                        dim: true
                        elide: Text.ElideRight
                    }
                }
                Row {
                    id: trailing
                    anchors.right: parent.right
                    anchors.rightMargin: 8
                    y: 6
                    spacing: 4
                    Toggle {
                        visible: row.d !== null && row.d.domain !== "climate"
                        enabled: row.d !== null && row.d.available
                        checked: row.d !== null && row.d.on
                        onToggled: on => panel.switchDevice(row.d, on)
                    }
                    IconButton {
                        visible: row.d !== null && row.d.domain === "climate"
                        icon: "list-remove-symbolic"
                        tooltip: "Half a degree cooler (Left)"
                        onClicked: panel.climate(row.d, -0.5)
                    }
                    IconButton {
                        visible: row.d !== null && row.d.domain === "climate"
                        icon: "list-add-symbolic"
                        tooltip: "Half a degree warmer (Right)"
                        onClicked: panel.climate(row.d, 0.5)
                    }
                }
                Slider {
                    visible: row.d !== null && row.d.dimmable && row.d.on
                    x: 38
                    y: 42
                    width: parent.width - 54
                    from: 1
                    to: 100
                    step: 5
                    value: row.d && row.d.brightness ? row.d.brightness * 100 : 0
                    onReleased: v => panel.brightness(row.d, v)
                }
                MouseArea {
                    anchors.fill: parent
                    z: -1
                    onClicked: panel.picked = panel.deviceRows.indexOf(row.index)
                }
            }
        }
        StateView {
            anchors.centerIn: parent
            width: parent.width
            visible: panel.rows.length === 0 && panel.home.devices !== undefined
            kind: "empty"
            icon: "go-home-symbolic"
            text: panel.filter === "" ? "No devices in Home yet" : "Nothing in Home matches “" + panel.host.query + "”"
        }
    }

    Item {
        id: footer
        anchors.bottom: parent.bottom
        width: parent.width
        height: openHome.height
        Row {
            spacing: 6
            Repeater {
                model: panel.failure === "" ? panel.routines.slice(0, 3) : []
                delegate: Button {
                    required property var modelData
                    text: "Run " + modelData.name
                    onClicked: panel.host.run("home.run", { routine: modelData.id }, (result, err) => {
                        if (result && result.message)
                            panel.host.say(result.message);
                    })
                }
            }
        }
        Button {
            id: openHome
            anchors.right: parent.right
            visible: panel.failure === ""
            text: "Open Home"
            onClicked: panel.host.open({})
        }
    }
}

import QtQuick
import Nyx.Ui

// This computer's containers and machines (Docker and Incus), running ones first; the
// words after `containers` keep those whose name starts with them. Up and Down pick
// one, Space starts or stops it, Ctrl+R restarts it, Enter opens it in Containers.
FocusScope {
    id: panel

    property var host: null
    readonly property var live: host ? host.topic("workloads") : ({})
    readonly property var all: (live.state && live.state.items) || []
    readonly property string filter: host ? host.query.trim().toLowerCase() : ""
    readonly property var shown: all.filter(w => filter === "" || w.title.toLowerCase().startsWith(filter))
    property int picked: 0
    readonly property var pickedOne: shown[Math.min(picked, shown.length - 1)] || null
    // Names being started or stopped, until Containers says it's done.
    property var busy: ({})

    function run(action, w) {
        if (!w)
            return;
        const b = Object.assign({}, busy);
        b[w.title] = true;
        busy = b;
        host.run("containers." + action, { name: w.title }, (result, err) => {
            const n = Object.assign({}, panel.busy);
            delete n[w.title];
            panel.busy = n;
            if (result)
                host.say(result.message);
        });
    }
    function toggle(w) {
        if (w)
            run(w.running ? "stop" : "start", w);
    }

    Keys.onPressed: e => {
        const ctrl = (e.modifiers & Qt.ControlModifier) !== 0;
        if (e.key === Qt.Key_Down)
            picked = Math.min(shown.length - 1, picked + 1);
        else if (e.key === Qt.Key_Up)
            picked = Math.max(0, picked - 1);
        else if (e.key === Qt.Key_Space)
            toggle(pickedOne);
        else if (ctrl && e.key === Qt.Key_R)
            run("restart", pickedOne);
        else if (e.key === Qt.Key_Return || e.key === Qt.Key_Enter)
            host.open(pickedOne ? { name: pickedOne.title } : {});
        else
            return;
        e.accepted = true;
    }

    ListView {
        id: list
        anchors.top: parent.top
        anchors.bottom: footer.top
        anchors.bottomMargin: 6
        width: parent.width
        clip: true
        model: panel.shown
        delegate: Rectangle {
            id: row
            required property var modelData
            required property int index
            readonly property bool working: panel.busy[modelData.title] === true
            width: list.width
            height: 46
            radius: Style.radiusSmall
            color: index === panel.picked ? Style.selected : "transparent"
            Accessible.role: Accessible.ListItem
            Accessible.name: modelData.title + ", " + modelData.state + ", " + modelData.subtitle
            Accessible.selected: index === panel.picked
            Rectangle {
                x: 12
                anchors.verticalCenter: parent.verticalCenter
                width: 8
                height: 8
                radius: 4
                color: row.modelData.running ? Style.success : row.modelData.state === "paused" ? Style.warning : Style.dimText
            }
            Column {
                x: 30
                anchors.verticalCenter: parent.verticalCenter
                width: parent.width - 30 - actions.width - 16
                Label {
                    width: parent.width
                    text: row.modelData.title
                    elide: Text.ElideRight
                }
                Label {
                    width: parent.width
                    text: row.modelData.subtitle + (row.modelData.project ? " · " + row.modelData.project : "")
                    role: "caption"
                    dim: true
                    elide: Text.ElideRight
                }
            }
            Row {
                id: actions
                anchors.right: parent.right
                anchors.rightMargin: 6
                anchors.verticalCenter: parent.verticalCenter
                spacing: 2
                Spinner {
                    visible: row.working
                    anchors.verticalCenter: parent.verticalCenter
                }
                IconButton {
                    visible: !row.working && row.modelData.running
                    icon: "view-refresh-symbolic"
                    tooltip: "Restart " + row.modelData.title + " (Ctrl+R)"
                    onClicked: panel.run("restart", row.modelData)
                }
                IconButton {
                    visible: !row.working
                    icon: row.modelData.running ? "media-playback-stop-symbolic" : "media-playback-start-symbolic"
                    tooltip: (row.modelData.running ? "Stop " : "Start ") + row.modelData.title + " (Space)"
                    onClicked: panel.toggle(row.modelData)
                }
            }
            MouseArea {
                anchors.fill: parent
                z: -1
                onClicked: panel.picked = row.index
                onDoubleClicked: panel.host.open({ name: row.modelData.title })
            }
        }
        StateView {
            anchors.centerIn: parent
            width: parent.width
            visible: panel.shown.length === 0 && panel.live.state !== undefined
            kind: "empty"
            icon: "application-x-executable-symbolic"
            text: panel.filter === "" ? "No containers on this computer" : "Nothing called “" + panel.host.query + "…”"
        }
    }

    Item {
        id: footer
        anchors.bottom: parent.bottom
        width: parent.width
        height: openApp.height
        Label {
            anchors.verticalCenter: parent.verticalCenter
            text: panel.all.filter(w => w.running).length + " of " + panel.all.length + " running"
            role: "caption"
            dim: true
        }
        Button {
            id: openApp
            anchors.right: parent.right
            text: "Open Containers"
            onClicked: panel.host.open(panel.pickedOne ? { name: panel.pickedOne.title } : {})
        }
    }
}

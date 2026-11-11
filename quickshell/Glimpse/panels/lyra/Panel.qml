import QtQuick
import Nyx.Ui

// Lyra (YouTube Music) at a glance: what's playing with its cover, the controls and
// volume, and the playlists (filtered by the words after `lyra`). Space plays or pauses,
// Left and Right skip, + and - change the volume, Up and Down pick a playlist, Enter plays
// it. Every control is answered once YouTube Music confirms it.
FocusScope {
    id: panel

    property var host: null
    // Across the top of Glimpse: what's playing on the left, the playlists beside it.
    readonly property bool wide: host !== null && host.compact
    readonly property int art: wide ? 80 : 96
    readonly property var playback: host ? host.topic("playback") : ({})
    readonly property var now: playback.state || ({})
    readonly property var track: now.track || null
    readonly property bool playing: now.state === "playing" || now.state === "buffering"
    readonly property bool ready: now.connection === "ready"
    readonly property var allLists: host && host.topic("playlists").state ? (host.topic("playlists").state.items || []) : []
    readonly property string filter: host ? host.query.toLowerCase() : ""
    readonly property var lists: filter === "" ? allLists : allLists.filter(p => p.name.toLowerCase().indexOf(filter) >= 0)
    property int picked: 0

    function control(action) {
        host.run("lyra.control", { action: action }, (result, err) => {
            if (err)
                host.say(err);
        });
    }
    function volume(level) {
        host.run("lyra.volume", { level: Math.max(0, Math.min(100, Math.round(level))) }, null);
    }
    function playList(i) {
        const p = lists[i];
        if (p)
            host.run("lyra.play", { playlist: p.id }, (result, err) => {
                host.say(err ? err : "Playing " + p.name);
            });
    }
    function clock(s) {
        s = Math.max(0, Math.floor(s || 0));
        const m = Math.floor(s / 60);
        return m + ":" + String(s % 60).padStart(2, "0");
    }

    Keys.onPressed: e => {
        if (e.key === Qt.Key_Space)
            control("toggle");
        else if (e.key === Qt.Key_Left)
            control("previous");
        else if (e.key === Qt.Key_Right)
            control("next");
        else if (e.key === Qt.Key_Plus || e.key === Qt.Key_Equal)
            volume((now.volume || 0) + 5);
        else if (e.key === Qt.Key_Minus)
            volume((now.volume || 0) - 5);
        else if (e.key === Qt.Key_Down)
            picked = Math.min(lists.length - 1, picked + 1);
        else if (e.key === Qt.Key_Up)
            picked = Math.max(0, picked - 1);
        else if (e.key === Qt.Key_Return || e.key === Qt.Key_Enter)
            playList(picked);
        else
            return;
        e.accepted = true;
    }
    onListsChanged: picked = Math.min(picked, Math.max(0, lists.length - 1))

    Column {
        id: top
        width: panel.wide ? 400 : parent.width
        spacing: panel.wide ? 8 : 12

        Label {
            text: panel.ready || !panel.track ? "Lyra · YouTube Music" : "Lyra · reconnecting to YouTube Music…"
            role: "caption"
            dim: true
        }

        Row {
            width: parent.width
            spacing: 14
            Rectangle {
                width: panel.art
                height: panel.art
                radius: Style.radiusCard
                color: Style.card
                clip: true
                Image {
                    anchors.fill: parent
                    visible: (panel.now.art || "") !== ""
                    source: visible ? "file://" + panel.now.art : ""
                    fillMode: Image.PreserveAspectCrop
                    asynchronous: true
                }
                Icon {
                    anchors.centerIn: parent
                    visible: (panel.now.art || "") === ""
                    name: "audio-x-generic-symbolic"
                    size: 36
                    color: Style.tertiary
                }
            }
            Column {
                width: parent.width - panel.art - 14
                anchors.verticalCenter: parent.verticalCenter
                spacing: 3
                Label {
                    width: parent.width
                    text: panel.track ? panel.track.title : (panel.now.signed_in === false ? "Sign in to YouTube Music" : "Nothing is playing")
                    role: "title"
                    elide: Text.ElideRight
                }
                Label {
                    width: parent.width
                    visible: panel.track !== null
                    text: panel.track ? panel.track.artist : ""
                    elide: Text.ElideRight
                }
                Label {
                    width: parent.width
                    text: panel.track ? (panel.track.album || "") : (panel.now.signed_in === false ? "Open Lyra to sign in." : "Pick a playlist, or type lyra and what to play.")
                    dim: true
                    wrapMode: panel.track ? Text.NoWrap : Text.Wrap
                    elide: Text.ElideRight
                }
            }
        }

        Column {
            width: parent.width
            spacing: 4
            visible: panel.track !== null && (panel.now.duration || 0) > 0
            ProgressBar {
                width: parent.width
                value: (panel.now.position || 0) / Math.max(1, panel.now.duration || 1)
            }
            Item {
                width: parent.width
                height: elapsed.implicitHeight
                Label {
                    id: elapsed
                    text: panel.clock(panel.now.position)
                    role: "caption"
                    dim: true
                }
                Label {
                    anchors.right: parent.right
                    text: panel.clock(panel.now.duration)
                    role: "caption"
                    dim: true
                }
            }
        }

        Row {
            spacing: 6
            IconButton {
                icon: "media-skip-backward-symbolic"
                tooltip: "Previous (Left)"
                enabled: panel.ready && panel.now.can_previous !== false
                onClicked: panel.control("previous")
            }
            IconButton {
                icon: panel.playing ? "media-playback-pause-symbolic" : "media-playback-start-symbolic"
                tooltip: panel.playing ? "Pause (Space)" : "Play (Space)"
                primary: true
                enabled: panel.ready && panel.track !== null
                onClicked: panel.control("toggle")
            }
            IconButton {
                icon: "media-skip-forward-symbolic"
                tooltip: "Next (Right)"
                enabled: panel.ready && panel.now.can_next !== false
                onClicked: panel.control("next")
            }
            Item {
                width: 8
                height: 1
            }
            Slider {
                width: panel.wide ? 160 : 180
                anchors.verticalCenter: parent.verticalCenter
                from: 0
                to: 100
                step: 5
                value: panel.now.volume || 0
                icon: "audio-volume-high-symbolic"
                enabled: panel.ready && panel.track !== null
                onReleased: v => panel.volume(v)
            }
        }
    }

    Label {
        id: heading
        x: panel.wide ? top.width + 16 : 0
        anchors.top: panel.wide ? parent.top : top.bottom
        anchors.topMargin: panel.wide ? 0 : 14
        text: panel.filter === "" ? "Your playlists" : "Playlists matching “" + panel.host.query + "”"
        role: "caption"
        dim: true
    }

    ListView {
        id: list
        x: heading.x
        anchors.top: heading.bottom
        anchors.topMargin: 4
        anchors.bottom: footer.top
        anchors.bottomMargin: 6
        width: parent.width - x
        clip: true
        model: panel.lists
        currentIndex: panel.picked
        delegate: ListRow {
            required property var modelData
            required property int index
            width: list.width
            icon: "view-list-symbolic"
            title: modelData.name
            selected: index === panel.picked
            onClicked: {
                panel.picked = index;
                panel.playList(index);
            }
        }
        StateView {
            anchors.centerIn: parent
            width: parent.width
            visible: panel.lists.length === 0
            kind: "empty"
            icon: "view-list-symbolic"
            text: panel.filter === "" ? "No playlists yet" : "No playlist matches “" + panel.host.query + "”"
        }
    }

    Item {
        id: footer
        anchors.bottom: parent.bottom
        width: parent.width
        height: openLyra.height
        Button {
            id: openLyra
            anchors.right: parent.right
            text: "Open Lyra"
            onClicked: panel.host.open({})
        }
    }
}

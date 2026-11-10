import QtQuick
import QtMultimedia
import Nyx.Ui

// Audio or video played in the preview, as the GTK panel's player: it starts paused;
// play/pause, the time played, a seek bar, the time left and the volume below it. Audio
// shows the file's icon above the controls. Ctrl+P plays and pauses.
Item {
    id: m

    property string path: ""
    property bool video: false
    property string icon: ""
    readonly property bool playing: player.playbackState === MediaPlayer.PlayingState

    function toggle() {
        if (playing) {
            player.pause();
        } else {
            // Played to the end: from the start again.
            if (player.duration > 0 && player.position >= player.duration - 50)
                player.position = 0;
            player.play();
        }
    }
    function clock(ms) {
        const s = Math.max(0, Math.round(ms / 1000));
        const h = Math.floor(s / 3600), mm = Math.floor(s / 60) % 60, ss = s % 60;
        return (h > 0 ? h + ":" + String(mm).padStart(2, "0") : mm) + ":" + String(ss).padStart(2, "0");
    }

    MediaPlayer {
        id: player
        source: m.path !== "" ? "file://" + m.path : ""
        audioOutput: AudioOutput {
            id: audio
            volume: 1
        }
        videoOutput: m.video ? screen : null
    }
    Component.onDestruction: player.stop()

    VideoOutput {
        id: screen
        visible: m.video
        width: parent.width
        height: parent.height - controls.height - 8
        fillMode: VideoOutput.PreserveAspectFit
    }
    Icon {
        visible: !m.video
        anchors.horizontalCenter: parent.horizontalCenter
        y: Math.max(0, (parent.height - controls.height - 8 - height) / 2)
        name: m.icon
        size: 64
        recolor: m.icon.endsWith("-symbolic")
        color: Style.dimText
    }

    Row {
        id: controls
        anchors.bottom: parent.bottom
        width: parent.width
        height: 36
        spacing: 8
        topPadding: 6

        PlayerButton {
            icon: m.playing ? "media-playback-pause-symbolic" : "media-playback-start-symbolic"
            onClicked: m.toggle()
        }
        Text {
            anchors.verticalCenter: parent.verticalCenter
            text: m.clock(player.position)
            color: Style.dimText
            font.family: Style.font
            font.pixelSize: 12
            font.features: { "tnum": 1 }
        }
        Slider {
            id: seek
            anchors.verticalCenter: parent.verticalCenter
            width: parent.width - 30 * 2 - 8 * 4 - 2 * 44 - 90
            from: 0
            to: Math.max(1, player.duration)
            step: 0
            value: player.position
            trackHeight: 4
            onReleased: v => player.position = v
        }
        Text {
            anchors.verticalCenter: parent.verticalCenter
            text: "-" + m.clock(Math.max(0, player.duration - player.position))
            color: Style.dimText
            font.family: Style.font
            font.pixelSize: 12
            font.features: { "tnum": 1 }
        }
        Slider {
            anchors.verticalCenter: parent.verticalCenter
            width: 90
            from: 0
            to: 100
            step: 5
            value: Math.round(audio.volume * 100)
            trackHeight: 4
            icon: audio.volume === 0 ? "audio-volume-muted-symbolic" : audio.volume < 0.34 ? "audio-volume-low-symbolic" : audio.volume < 0.67 ? "audio-volume-medium-symbolic" : "audio-volume-high-symbolic"
            onMoved: v => audio.volume = v / 100
            onReleased: v => audio.volume = v / 100
            onIconClicked: audio.volume = audio.volume > 0 ? 0 : 1
        }
    }

    component PlayerButton: Rectangle {
        id: pb
        property string icon: ""
        signal clicked
        anchors.verticalCenter: parent.verticalCenter
        width: 30
        height: 30
        radius: Style.radiusSmall
        color: pbArea.containsMouse ? Style.rowHover : "transparent"
        Icon {
            anchors.centerIn: parent
            name: pb.icon
            size: 16
            color: pbArea.containsMouse ? Style.text : Style.dimText
        }
        MouseArea {
            id: pbArea
            anchors.fill: parent
            hoverEnabled: true
            onClicked: pb.clicked()
        }
    }
}

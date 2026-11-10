import QtQuick
import Nyx.Ui

// One of a plugin's settings, as the plugin describes it (entry, switch, slider or
// choice), with its own Save.
Column {
    id: s

    required property var modelData
    readonly property string kind: modelData.kind || "entry"
    readonly property var options: (modelData.options || []).slice(0, 128)
    readonly property bool sliderOk: modelData.min !== undefined && modelData.max !== undefined && modelData.min < modelData.max
    property var value: modelData.value

    signal save(var value)

    spacing: 8

    Label {
        width: parent.width
        text: s.modelData.title || s.modelData.key
        strong: true
    }
    Label {
        visible: text !== ""
        width: parent.width
        text: s.modelData.desc || ""
        wrapMode: Text.Wrap
        role: "secondary"
        dim: true
    }

    Field {
        visible: s.kind === "entry"
        width: parent.width
        text: s.kind === "entry" ? (s.modelData.value || "") : ""
        onTextChanged: if (s.kind === "entry")
            s.value = text
    }
    Toggle {
        visible: s.kind === "switch"
        checked: s.modelData.value === true
        onToggled: on => s.value = on
    }
    Row {
        visible: s.kind === "slider" && s.sliderOk
        width: parent.width
        spacing: 12
        Slider {
            id: slider
            anchors.verticalCenter: parent.verticalCenter
            width: parent.width - 60
            from: s.sliderOk ? s.modelData.min : 0
            to: s.sliderOk ? s.modelData.max : 1
            step: s.modelData.step || 1
            value: s.kind === "slider" ? Number(s.modelData.value !== undefined ? s.modelData.value : from) : 0
            onMoved: v => s.value = v
            onReleased: v => s.value = v
        }
        Label {
            anchors.verticalCenter: parent.verticalCenter
            text: String(Math.round(Number(s.value) * 100) / 100)
            dim: true
        }
    }
    // A few choices side by side; more as a list.
    Segmented {
        visible: s.kind === "choice" && s.options.length > 0 && s.options.length <= 4
        width: Math.min(parent.width, s.options.length * 120)
        options: s.options.map(o => ({ id: o[0], label: o[1] }))
        current: String(s.value)
        onChosen: id => s.value = id
    }
    Column {
        visible: s.kind === "choice" && s.options.length > 4
        width: parent.width
        Repeater {
            model: s.kind === "choice" && s.options.length > 4 ? s.options : []
            delegate: ListRow {
                required property var modelData
                width: parent.width
                title: modelData[1]
                selected: String(s.value) === modelData[0]
                onClicked: s.value = modelData[0]
            }
        }
    }
    Label {
        visible: ["entry", "switch", "slider", "choice"].indexOf(s.kind) < 0
        text: "This setting can't be shown here."
        dim: true
    }

    Button {
        visible: ["entry", "switch", "slider", "choice"].indexOf(s.kind) >= 0
        text: "Save " + (s.modelData.title || s.modelData.key)
        onClicked: s.save(s.value)
    }
}
